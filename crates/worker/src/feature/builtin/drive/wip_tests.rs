//! Model-facing integration through production Inspect/Invoke/Tree, native descriptors,
//! result validation and the Drive adapter. Only Workspace transport/persistence is replaced
//! by isolated state; this suite does not prove Backend grants or DB durability. Local scoped
//! Workdir sessions exercise source reads. Runs in the normal `worker --lib` test target.
use super::*;
use crate::wip::{WipMountRegistry, WipRuntime, WipSubtreeProvider};
use crate::worker::{
    WorkspaceBinaryRequest, WorkspaceBinaryResponse, WorkspaceClientError, WorkspaceRequest,
    WorkspaceResponse,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use wip_protocol::InterfaceReference;

const NATIVE: &str = "yoi.drive/entry/v2";

fn execution() -> ToolExecutionContext {
    ToolExecutionContext::new(uuid::Uuid::now_v7().to_string(), "drive-model-test", 0)
}

#[derive(Debug)]
struct Node {
    metadata: DriveEntry,
    bytes: Vec<u8>,
}
#[derive(Debug)]
struct State {
    nodes: BTreeMap<String, Node>,
    next_id: u64,
    receipts: BTreeMap<String, DriveMutationResponse>,
    mutations: Vec<DriveMutationRequest>,
    uploads: Vec<(DriveUploadQuery, Vec<u8>)>,
    requests: Vec<String>,
    denied_metadata: BTreeSet<String>,
    deny_created_inspect: bool,
    revoked: bool,
    write: bool,
    lose_next_response: bool,
}
#[derive(Debug)]
struct Client(Mutex<State>);
fn response(value: impl Serialize) -> WorkspaceResponse {
    WorkspaceResponse {
        status: 200,
        body: serde_json::to_string(&value).unwrap(),
    }
}
fn failure(status: u16, code: DriveApiErrorCode) -> WorkspaceResponse {
    WorkspaceResponse {
        status,
        body: serde_json::to_string(&DriveApiError::new(code)).unwrap(),
    }
}
fn reference(id: &str) -> DriveEntryRef {
    DriveEntryRef {
        workspace_id: "workspace".into(),
        node_id: id.into(),
    }
}
fn node(id: &str, parent: Option<&str>, name: &str, bytes: Option<&[u8]>) -> Node {
    Node {
        metadata: DriveEntry {
            entry: reference(id),
            parent: parent.map(reference),
            name: name.into(),
            kind: if bytes.is_some() {
                DriveEntryKind::File
            } else {
                DriveEntryKind::Folder
            },
            last_mutation_id: "initial".into(),
            size: bytes.map(|b| b.len() as u32),
            content_type: bytes.map(|_| "text/markdown".into()),
            updated_by: "fixture".into(),
            updated_at: "now".into(),
            latest_url: format!("/api/w/workspace/drive/download?id={id}"),
        },
        bytes: bytes.unwrap_or_default().into(),
    }
}
impl Client {
    fn new() -> Arc<Self> {
        let nodes = [
            node("1", None, "root", None),
            node("2", Some("1"), "documents", None),
            node("3", Some("2"), "alpha.md", Some(b"alpha body")),
            node("4", Some("2"), "beta.md", Some(b"beta body")),
            node(
                "5",
                Some("2"),
                "image.png",
                Some(b"\x89PNG\r\n\x1a\nfixture"),
            ),
        ]
        .into_iter()
        .map(|n| (n.metadata.entry.node_id.clone(), n))
        .collect();
        Arc::new(Self(Mutex::new(State {
            nodes,
            next_id: 9007199254740993,
            receipts: BTreeMap::new(),
            mutations: Vec::new(),
            uploads: Vec::new(),
            requests: Vec::new(),
            denied_metadata: BTreeSet::new(),
            deny_created_inspect: false,
            revoked: false,
            write: true,
            lose_next_response: false,
        })))
    }
}
fn query(path: &str) -> (String, BTreeMap<String, String>) {
    let url = reqwest::Url::parse(&format!("https://fixture.invalid{path}")).unwrap();
    (
        url.path()
            .strip_prefix("/api/w/workspace/drive/")
            .unwrap()
            .into(),
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect(),
    )
}
impl State {
    fn create(
        &mut self,
        parent: &DriveEntryRef,
        name: &str,
        bytes: Option<&[u8]>,
        request_id: &str,
    ) -> DriveEntry {
        assert_eq!(parent.workspace_id, "workspace");
        assert_eq!(
            self.nodes[&parent.node_id].metadata.kind,
            DriveEntryKind::Folder
        );
        let id = self.next_id.to_string();
        self.next_id += 1;
        let mut node = node(&id, Some(&parent.node_id), name, bytes);
        node.metadata.last_mutation_id = request_id.into();
        let entry = node.metadata.clone();
        self.nodes.insert(id.clone(), node);
        if self.deny_created_inspect {
            self.denied_metadata.insert(id);
        }
        entry
    }
    fn commit(
        &mut self,
        receipt: DriveMutationResponse,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.receipts
            .insert(receipt.request_id.clone(), receipt.clone());
        if std::mem::take(&mut self.lose_next_response) {
            Err(WorkspaceClientError::Request(
                "injected response loss after commit".into(),
            ))
        } else {
            Ok(response(receipt))
        }
    }
    fn mutate(
        &mut self,
        request: DriveMutationRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.mutations.push(request.clone());
        if !self.write {
            return Ok(failure(403, DriveApiErrorCode::Denied));
        }
        let rid = &request.request_id;
        let entry = match &request.mutation {
            DriveMutation::CreateFolder { parent, name } => {
                Some(self.create(parent, name, None, rid))
            }
            DriveMutation::CreateText {
                parent, name, text, ..
            } => Some(self.create(parent, name, Some(text.as_bytes()), rid)),
            DriveMutation::UpdateText {
                id,
                expected_mutation_id,
                ..
            }
            | DriveMutation::Relocate {
                id,
                expected_mutation_id,
                ..
            }
            | DriveMutation::Delete {
                id,
                expected_mutation_id,
            } => {
                let Some(current) = self.nodes.get(&id.node_id) else {
                    return Ok(failure(404, DriveApiErrorCode::NotFound));
                };
                assert_eq!(id.workspace_id, "workspace");
                if current.metadata.last_mutation_id != *expected_mutation_id {
                    return Ok(failure(409, DriveApiErrorCode::Conflict));
                }
                if matches!(request.mutation, DriveMutation::Delete { .. }) {
                    self.nodes.remove(&id.node_id);
                    None
                } else {
                    let current = self.nodes.get_mut(&id.node_id).unwrap();
                    match &request.mutation {
                        DriveMutation::Relocate { parent, name, .. } => {
                            current.metadata.parent = Some(parent.clone());
                            current.metadata.name = name.clone();
                        }
                        DriveMutation::UpdateText { text, .. } => {
                            current.bytes = text.as_bytes().into();
                            current.metadata.size = Some(text.len() as u32);
                        }
                        _ => unreachable!(),
                    }
                    current.metadata.last_mutation_id = rid.clone();
                    Some(current.metadata.clone())
                }
            }
        };
        self.commit(DriveMutationResponse {
            request_id: rid.clone(),
            entry,
        })
    }
}
impl WorkspaceClient for Client {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "drive-stateful-native-test"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let mut state = self.0.lock().unwrap();
        if request.path.ends_with("/authorization/check") {
            let input: server_api::authorization::OperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            return Ok(if state.revoked {
                failure(403, DriveApiErrorCode::Denied)
            } else {
                response(json!({"scope":input.resource}))
            });
        }
        state.requests.push(request.path.clone());
        if state.revoked {
            return Ok(failure(403, DriveApiErrorCode::Denied));
        }
        let (operation, args) = query(&request.path);
        Ok(match operation.as_str() {
            "capabilities" => response(DriveCapabilities { write: state.write }),
            "root" => response(&state.nodes["1"].metadata),
            "metadata" | "read-text" => {
                assert_eq!(args["entry_workspace_id"], "workspace");
                let id = &args["id"];
                if state.denied_metadata.contains(id) {
                    return Ok(failure(403, DriveApiErrorCode::Denied));
                }
                let Some(node) = state.nodes.get(id) else {
                    return Ok(failure(404, DriveApiErrorCode::NotFound));
                };
                if operation == "metadata" {
                    response(&node.metadata)
                } else {
                    let max: usize = args["max_bytes"].parse().unwrap();
                    let text = String::from_utf8(node.bytes.clone()).unwrap();
                    response(DriveReadTextResponse {
                        entry: node.metadata.clone(),
                        text: text.chars().take(max).collect(),
                        truncated: text.len() > max,
                    })
                }
            }
            "list" | "search" => {
                let limit = args.get("limit").map(|s| s.parse().unwrap()).unwrap_or(200);
                let offset = args
                    .get("after")
                    .map(|s| s.strip_prefix("page:").unwrap().parse::<usize>().unwrap())
                    .unwrap_or(0);
                let matches: Vec<_> = state
                    .nodes
                    .values()
                    .filter(|node| {
                        if operation == "list" {
                            node.metadata
                                .parent
                                .as_ref()
                                .is_some_and(|p| p.node_id == args["id"])
                        } else {
                            node.metadata.name.contains(&args["query"])
                        }
                    })
                    .collect();
                let entries = matches
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .map(|n| n.metadata.clone())
                    .collect();
                let next_after =
                    (offset + limit < matches.len()).then(|| format!("page:{}", offset + limit));
                response(DriveListResponse {
                    entries,
                    next_after,
                })
            }
            "mutate" => {
                return state
                    .mutate(serde_json::from_str(request.body.as_deref().unwrap()).unwrap());
            }
            operation if operation.starts_with("requests/") => {
                let id = operation.strip_prefix("requests/").unwrap();
                let receipt = state.receipts.get(id).cloned();
                response(DriveRequestStatusResponse {
                    request_id: id.into(),
                    state: if receipt.is_some() {
                        DriveRequestState::Committed
                    } else {
                        DriveRequestState::Uncommitted
                    },
                    response: receipt,
                })
            }
            _ => panic!("unexpected fixture request: {}", request.path),
        })
    }
    fn execute_binary(
        &self,
        request: WorkspaceBinaryRequest,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        let mut state = self.0.lock().unwrap();
        state.requests.push(request.path.clone());
        let (operation, args) = query(&request.path);
        assert_eq!(args["entry_workspace_id"], "workspace");
        if state.revoked || (operation == "upload" && !state.write) {
            let denied = failure(403, DriveApiErrorCode::Denied);
            return Ok(WorkspaceBinaryResponse {
                status: denied.status,
                body: denied.body.into_bytes(),
            });
        }
        if operation == "read-chunk" {
            let node = &state.nodes[&args["id"]];
            assert_eq!(args["expected_mutation_id"], node.metadata.last_mutation_id);
            let offset: usize = args["offset"].parse().unwrap();
            let length: usize = args["length"].parse().unwrap();
            return Ok(WorkspaceBinaryResponse {
                status: 200,
                body: node.bytes[offset..(offset + length).min(node.bytes.len())].to_vec(),
            });
        }
        assert_eq!(operation, "upload");
        let bytes = request.body.unwrap();
        let upload = DriveUploadQuery {
            operation: DriveUploadOperation::Create,
            request_id: args["request_id"].clone(),
            entry_workspace_id: args["entry_workspace_id"].clone(),
            parent_id: Some(args["parent_id"].clone()),
            name: Some(args["name"].clone()),
            id: None,
            expected_mutation_id: None,
            content_type: args["content_type"].clone(),
            size: args["size"].parse().unwrap(),
            sha256: args["sha256"].clone(),
        };
        assert_eq!(args["operation"], "create");
        assert_eq!(upload.size as usize, bytes.len());
        assert_eq!(upload.sha256, digest(&bytes));
        let entry = state.create(
            &reference(upload.parent_id.as_deref().unwrap()),
            upload.name.as_deref().unwrap(),
            Some(&bytes),
            &upload.request_id,
        );
        let receipt = DriveMutationResponse {
            request_id: upload.request_id.clone(),
            entry: Some(entry),
        };
        state.uploads.push((upload, bytes));
        let result = state.commit(receipt)?;
        Ok(WorkspaceBinaryResponse {
            status: result.status,
            body: result.body.into_bytes(),
        })
    }
}
fn runtime(client: Arc<Client>, router: Arc<WorkdirSessionRouter>) -> (WipRuntime, DriveFeature) {
    let feature = DriveFeature::new(client, router);
    let mut mounts = WipMountRegistry::new();
    wip::mount_drive_wip(&mut mounts, &feature).unwrap();
    (
        WipRuntime::from_mounts(mounts, "drive-model-tests".into()).unwrap(),
        feature,
    )
}
fn output(output: ToolOutput) -> Value {
    serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
}
async fn inspect(runtime: &WipRuntime, path: &str) -> Value {
    output(runtime.inspect(path.into(), true).await.unwrap())
}
async fn native(runtime: &WipRuntime, path: &str) -> (InterfaceReference, String) {
    let object = inspect(runtime, path).await;
    let interface = object["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["reference"]["name"] == NATIVE)
        .unwrap();
    let declaration = inspect(runtime, interface["path"].as_str().unwrap()).await;
    (
        InterfaceReference {
            scope: interface["reference"]["scope"].as_str().unwrap().into(),
            name: interface["reference"]["name"].as_str().unwrap().into(),
        },
        declaration["interface_signature"].as_str().unwrap().into(),
    )
}
async fn invoke(
    runtime: &WipRuntime,
    path: &str,
    interface: &InterfaceReference,
    op: &str,
    args: Value,
) -> Value {
    output(
        runtime
            .invoke(path.into(), interface.clone(), op.into(), args, execution())
            .await
            .unwrap(),
    )
}
async fn root(runtime: &WipRuntime) -> (InterfaceReference, Value) {
    let (service, _) = native(runtime, "/drive").await;
    let root = invoke(runtime, "/drive", &service, "root", json!({})).await;
    assert!(root["path"].is_string());
    (service, root)
}
fn path(entry: &Value) -> &str {
    entry["path"].as_str().expect("typed Entry path")
}

#[tokio::test]
async fn service_and_root_have_distinct_identity_scope_and_official_typed_signatures() {
    let client = Client::new();
    let (runtime, feature) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, root) = root(&runtime).await;
    let (folder, folder_signature) = native(&runtime, path(&root)).await;
    let (_, service_signature) = native(&runtime, "/drive").await;
    assert_eq!(service.scope, "/drive");
    assert_eq!(folder.scope, path(&root));
    assert_ne!(service, folder);
    for op in [
        "root(",
        "search(",
        "request_status(",
        "list(",
        "save_workdir(",
    ] {
        assert!(service_signature.contains(op), "{service_signature}");
    }
    assert!(!folder_signature.contains("root("));
    assert!(!folder_signature.contains("search("));
    assert!(!folder_signature.contains("request_status("));
    assert!(folder_signature.contains("list("));
    assert!(
        service_signature.contains("path: entry"),
        "{service_signature}"
    );
    assert!(
        service_signature.contains("metadata: json"),
        "{service_signature}"
    );
    assert!(
        service_signature.contains("next_after?: string"),
        "{service_signature}"
    );
    assert!(
        service_signature.contains("request_id: string"),
        "{service_signature}"
    );
    assert!(service_signature.contains("uncommitted"));
    assert!(service_signature.contains("committed"));
    assert!(service_signature.contains("response?"));
    let provider = wip::DriveProvider::new(feature);
    let service = provider.publication("/drive").await.unwrap().unwrap();
    let folder = provider.publication(path(&root)).await.unwrap().unwrap();
    assert_eq!(
        service.projection.object.r#ref.as_deref(),
        Some("drive-service:workspace")
    );
    assert_eq!(
        folder.projection.object.r#ref.as_deref(),
        Some("drive:workspace:1")
    );
    assert_ne!(
        service.projection.object.r#ref,
        folder.projection.object.r#ref
    );
    assert_eq!(
        invoke(
            &runtime,
            path(&root),
            &folder.projection.interface,
            "metadata",
            json!({})
        )
        .await,
        root
    );
    let state = client.0.lock().unwrap();
    assert!(
        !state
            .requests
            .iter()
            .any(|p| p.contains("read-text") || p.contains("read-chunk"))
    );
}

#[tokio::test]
async fn returned_paths_navigate_paged_list_search_and_read_without_indexable_expansion() {
    let client = Client::new();
    let (runtime, _) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, root) = root(&runtime).await;
    let (root_interface, _) = native(&runtime, path(&root)).await;
    let children = invoke(
        &runtime,
        path(&root),
        &root_interface,
        "list",
        json!({"limit":1}),
    )
    .await;
    assert!(
        children.get("next_after").is_none(),
        "null optional cursor must be omitted"
    );
    let folder = &children["entries"][0];
    let (folder_interface, _) = native(&runtime, path(folder)).await;
    let first = invoke(
        &runtime,
        path(folder),
        &folder_interface,
        "list",
        json!({"limit":1}),
    )
    .await;
    assert_eq!(first["entries"].as_array().unwrap().len(), 1);
    let second = invoke(
        &runtime,
        path(folder),
        &folder_interface,
        "list",
        json!({"limit":1,"after":first["next_after"]}),
    )
    .await;
    assert_ne!(path(&first["entries"][0]), path(&second["entries"][0]));
    let found = invoke(
        &runtime,
        "/drive",
        &service,
        "search",
        json!({"query":".md","limit":1}),
    )
    .await;
    let next = invoke(
        &runtime,
        "/drive",
        &service,
        "search",
        json!({"query":".md","limit":1,"after":found["next_after"]}),
    )
    .await;
    assert!(next.get("next_after").is_none());
    assert_eq!(next["entries"][0], second["entries"][0]);
    assert!(
        !client
            .0
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|p| p.contains("read-text"))
    );
    assert!(
        !client.0.lock().unwrap().requests.iter().any(|p| {
            let (operation, args) = query(p);
            operation == "metadata" && ["3", "4", "5"].contains(&args["id"].as_str())
        }),
        "page results must not eagerly acquire unselected entries"
    );
    let selected = path(&next["entries"][0]);
    let (file, signature) = native(&runtime, selected).await;
    assert!(signature.contains("read("));
    assert!(signature.contains("view_image("));
    assert!(signature.contains("parent: entry"));
    assert!(!signature.contains("create_text("));
    let read = invoke(&runtime, selected, &file, "read", json!({})).await;
    assert_eq!(path(&read["entry"]), selected);
    assert_eq!(read["text"], "beta body");
    assert_eq!(read["truncated"], false);
    for start in ["/", "/drive"] {
        let tree = output(runtime.tree(start.into(), 8, true).await.unwrap());
        assert_eq!(tree["coverage"]["complete"], true);
        assert!(!tree["tree"].to_string().contains(selected));
    }
    assert_eq!(
        client
            .0
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|p| p.contains("read-text"))
            .count(),
        1
    );
}

#[tokio::test]
async fn creation_and_receipts_return_entries_and_relocate_uses_returned_folder_not_service() {
    let client = Client::new();
    let (runtime, feature) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, _) = root(&runtime).await;
    let created = invoke(
        &runtime,
        "/drive",
        &service,
        "create_text",
        json!({"name":"created.md","content":"created body"}),
    )
    .await;
    let created_path = path(&created["entry"]).to_owned();
    let (file, _) = native(&runtime, &created_path).await;
    let before = wip::DriveProvider::new(feature.clone())
        .publication(&created_path)
        .await
        .unwrap()
        .unwrap()
        .projection
        .object
        .r#ref;
    assert_eq!(
        invoke(&runtime, &created_path, &file, "read", json!({})).await["text"],
        "created body"
    );
    let folder = invoke(
        &runtime,
        "/drive",
        &service,
        "create_folder",
        json!({"name":"destination"}),
    )
    .await;
    let folder_path = path(&folder["entry"]).to_owned();
    let status = invoke(
        &runtime,
        "/drive",
        &service,
        "request_status",
        json!({"request_id":created["request_id"]}),
    )
    .await;
    assert_eq!(status["state"], "committed");
    assert_eq!(status["response"]["entry"], created["entry"]);
    let uncommitted = invoke(
        &runtime,
        "/drive",
        &service,
        "request_status",
        json!({"request_id":"never-submitted"}),
    )
    .await;
    assert_eq!(uncommitted["state"], "uncommitted");
    assert!(uncommitted.get("response").is_none());
    let count = client.0.lock().unwrap().mutations.len();
    assert!(
        runtime
            .invoke(
                created_path.clone(),
                file.clone(),
                "relocate".into(),
                json!({"parent":"/drive","name":"moved.md"}),
                execution()
            )
            .await
            .is_err()
    );
    assert_eq!(client.0.lock().unwrap().mutations.len(), count);
    let moved = invoke(
        &runtime,
        &created_path,
        &file,
        "relocate",
        json!({"parent":folder_path,"name":"moved.md"}),
    )
    .await;
    assert_eq!(path(&moved["entry"]), created_path);
    assert_eq!(moved["entry"]["metadata"]["name"], "moved.md");
    let after = wip::DriveProvider::new(feature)
        .publication(&created_path)
        .await
        .unwrap()
        .unwrap()
        .projection
        .object
        .r#ref;
    assert_eq!(before, after);
    let (folder_interface, _) = native(&runtime, &folder_path).await;
    let list = invoke(&runtime, &folder_path, &folder_interface, "list", json!({})).await;
    assert_eq!(list["entries"][0], moved["entry"]);
    assert_eq!(
        invoke(&runtime, &created_path, &file, "read", json!({})).await["text"],
        "created body"
    );
}

#[tokio::test]
async fn committed_create_survives_selected_entry_inspect_denial_and_is_never_replayed() {
    let client = Client::new();
    client.0.lock().unwrap().deny_created_inspect = true;
    client.0.lock().unwrap().lose_next_response = true;
    let (runtime, _) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, _) = root(&runtime).await;
    let created = invoke(
        &runtime,
        "/drive",
        &service,
        "create_text",
        json!({"name":"one.md","content":"one"}),
    )
    .await;
    assert!(
        runtime
            .inspect(path(&created["entry"]).into(), true)
            .await
            .is_err()
    );
    let status = invoke(
        &runtime,
        "/drive",
        &service,
        "request_status",
        json!({"request_id":created["request_id"]}),
    )
    .await;
    assert_eq!(status["response"]["entry"], created["entry"]);
    let state = client.0.lock().unwrap();
    assert_eq!(state.mutations.len(), 1);
    assert_eq!(state.receipts.len(), 1);
    assert_eq!(
        state
            .nodes
            .values()
            .filter(|n| n.metadata.name == "one.md")
            .count(),
        1
    );
    assert_eq!(state.mutations[0].request_id, created["request_id"]);
}

#[tokio::test]
async fn delete_receipt_has_no_live_entry_and_same_name_recreation_has_new_identity() {
    let client = Client::new();
    let (runtime, _) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, _) = root(&runtime).await;
    let old = invoke(
        &runtime,
        "/drive",
        &service,
        "create_text",
        json!({"name":"same.md","content":"old"}),
    )
    .await;
    let old_path = path(&old["entry"]);
    let (file, _) = native(&runtime, old_path).await;
    let deleted = invoke(&runtime, old_path, &file, "delete", json!({})).await;
    assert!(deleted.get("entry").is_none());
    let status = invoke(
        &runtime,
        "/drive",
        &service,
        "request_status",
        json!({"request_id":deleted["request_id"]}),
    )
    .await;
    assert!(status["response"].get("entry").is_none());
    assert!(runtime.inspect(old_path.into(), true).await.is_err());
    let new = invoke(
        &runtime,
        "/drive",
        &service,
        "create_text",
        json!({"name":"same.md","content":"new"}),
    )
    .await;
    assert_ne!(old_path, path(&new["entry"]));
    assert!(
        runtime
            .invoke(old_path.into(), file, "read".into(), json!({}), execution())
            .await
            .is_err()
    );
    let (file, _) = native(&runtime, path(&new["entry"])).await;
    assert_eq!(
        invoke(&runtime, path(&new["entry"]), &file, "read", json!({})).await["text"],
        "new"
    );
}

#[tokio::test]
async fn scoped_workdir_upload_returns_readable_entry_and_denied_sources_do_not_upload() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("artifact.md"), "artifact body").unwrap();
    std::fs::write(dir.path().join("secret.md"), "not shared").unwrap();
    let scope = manifest::Scope::from_config(
        &serde_json::from_value(json!({
            "allow": [{"target": dir.path().join("artifact.md"), "permission": "read"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let session = Arc::new(workdir::LocalWorkdirSession::materialized(
        dir.path().into(),
        dir.path().into(),
        manifest::SharedScope::new(scope),
        workdir::WorkdirSessionCapabilities::READ_ONLY,
    ));
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            workdir::WorkdirAttachmentAlias::new("artifacts").unwrap(),
            session,
        )
        .unwrap();
    let client = Client::new();
    let (runtime, _) = runtime(client.clone(), router);
    let (service, _) = root(&runtime).await;
    for (alias, source) in [
        ("missing", "artifact.md"),
        ("artifacts", "secret.md"),
        ("artifacts", "../artifact.md"),
    ] {
        assert!(
            runtime
                .invoke(
                    "/drive".into(),
                    service.clone(),
                    "save_workdir".into(),
                    json!({"target_workdir":alias,"path":source,"name":"saved.md"}),
                    execution()
                )
                .await
                .is_err()
        );
    }
    assert!(client.0.lock().unwrap().uploads.is_empty());
    let saved = invoke(&runtime, "/drive", &service, "save_workdir", json!({"target_workdir":"artifacts","path":"artifact.md","name":"saved.md","content_type":"text/markdown"})).await;
    let (file, _) = native(&runtime, path(&saved["entry"])).await;
    assert_eq!(
        invoke(&runtime, path(&saved["entry"]), &file, "read", json!({})).await["text"],
        "artifact body"
    );
    let state = client.0.lock().unwrap();
    assert_eq!(state.uploads.len(), 1);
    assert_eq!(state.uploads[0].1, b"artifact body");
    assert_eq!(state.uploads[0].0.request_id, saved["request_id"]);
    assert!(
        state
            .receipts
            .contains_key(saved["request_id"].as_str().unwrap())
    );
}

#[tokio::test]
async fn image_result_keeps_typed_entry_and_real_attachment() {
    let client = Client::new();
    let (runtime, _) = runtime(client, Arc::new(WorkdirSessionRouter::new()));
    let (service, _) = root(&runtime).await;
    let found = invoke(
        &runtime,
        "/drive",
        &service,
        "search",
        json!({"query":"image.png"}),
    )
    .await;
    let image_path = path(&found["entries"][0]);
    let (file, signature) = native(&runtime, image_path).await;
    assert!(signature.contains("attached: bool"), "{signature}");
    assert!(signature.contains("mime_type: string"));
    assert!(signature.contains("`bytes`: integer"), "{signature}");
    let result = runtime
        .invoke(
            image_path.into(),
            file,
            "view_image".into(),
            json!({}),
            execution(),
        )
        .await
        .unwrap();
    let Attachment::Image(image) = &result.attachments[0];
    assert_eq!(image.data(), b"\x89PNG\r\n\x1a\nfixture");
    let result = output(result);
    assert_eq!(path(&result["entry"]), image_path);
    assert_eq!(result["attached"], true);
    assert_eq!(result["mime_type"], "image/png");
    assert_eq!(result["bytes"], 15);
}

// Retain only the previous service binding while keeping the current descriptor
// and validator. This isolates scope_ref fencing from the v1 membership change.
struct PreviousServiceBinding {
    provider: wip::DriveProvider,
    previous: Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait]
impl WipSubtreeProvider for PreviousServiceBinding {
    async fn publication(
        &self,
        path: &str,
    ) -> Result<Option<crate::wip::WipPublication>, wip_protocol::ProtocolError> {
        let Some(mut publication) = self.provider.publication(path).await? else {
            return Ok(None);
        };
        if path == "/drive" && self.previous.load(std::sync::atomic::Ordering::SeqCst) {
            publication.projection.object.r#ref = Some("drive:workspace:1".into());
            publication = crate::wip::WipPublication::self_scoped(publication.projection)?;
        }
        Ok(Some(publication))
    }
    async fn children(&self, _: &str) -> Result<Vec<String>, wip_protocol::ProtocolError> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn previous_service_scope_binding_and_v1_interface_cannot_dispatch_creation() {
    let client = Client::new();
    let feature = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let provider = wip::DriveProvider::new(feature);
    let current = provider
        .publication("/drive")
        .await
        .unwrap()
        .unwrap()
        .projection;
    let previous = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut mounts = WipMountRegistry::new();
    mounts.allocate_namespace("drive", "drive").unwrap();
    mounts.mount(current).unwrap();
    mounts
        .mount_subtree(crate::wip::WipSubtreeMount {
            root: "/drive".into(),
            provider: Arc::new(PreviousServiceBinding {
                provider,
                previous: previous.clone(),
            }),
        })
        .unwrap();
    let runtime = WipRuntime::from_mounts(mounts, "previous-drive-binding".into()).unwrap();
    let (service, _) = native(&runtime, "/drive").await;
    previous.store(false, std::sync::atomic::Ordering::SeqCst);
    let error = runtime
        .call(
            "/drive".into(),
            service.clone(),
            "create_folder".into(),
            json!({"name":"not-created"}),
            execution(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("InterfaceMismatch"), "{error}");
    assert!(client.0.lock().unwrap().mutations.is_empty());
    let v1 = InterfaceReference {
        scope: service.scope.clone(),
        name: "yoi.drive/entry/v1".into(),
    };
    assert!(
        runtime
            .invoke(
                "/drive".into(),
                v1,
                "create_folder".into(),
                json!({"name":"not-created"}),
                execution()
            )
            .await
            .is_err()
    );
    assert!(client.0.lock().unwrap().mutations.is_empty());
    let (fresh, _) = native(&runtime, "/drive").await;
    invoke(
        &runtime,
        "/drive",
        &fresh,
        "create_folder",
        json!({"name":"explicit-fresh-call"}),
    )
    .await;
    assert_eq!(client.0.lock().unwrap().mutations.len(), 1);
}

#[tokio::test]
async fn stale_entry_observation_is_rejected_before_mutation_without_automatic_retry() {
    let client = Client::new();
    let (runtime, _) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, root) = root(&runtime).await;
    let created = invoke(
        &runtime,
        "/drive",
        &service,
        "create_text",
        json!({"name":"external.md","content":"observed"}),
    )
    .await;
    let selected = path(&created["entry"]);
    let (file, _) = native(&runtime, selected).await;
    let id = created["entry"]["metadata"]["entry"]["node_id"]
        .as_str()
        .unwrap();
    client
        .0
        .lock()
        .unwrap()
        .nodes
        .get_mut(id)
        .unwrap()
        .metadata
        .last_mutation_id = "external-commit".into();
    let count = client.0.lock().unwrap().mutations.len();
    let error = runtime
        .call(
            selected.into(),
            file.clone(),
            "relocate".into(),
            json!({"parent":path(&root),"name":"should-not-move.md"}),
            execution(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("ValidatorMismatch"), "{error}");
    assert_eq!(client.0.lock().unwrap().mutations.len(), count);
    let (fresh, _) = native(&runtime, selected).await;
    let read = invoke(&runtime, selected, &fresh, "read", json!({})).await;
    assert_eq!(
        read["entry"]["metadata"]["last_mutation_id"],
        "external-commit"
    );
    assert_eq!(read["entry"]["metadata"]["name"], "external.md");
}

#[tokio::test]
async fn revocation_removes_selected_entry_access_without_reading_or_replaying() {
    let client = Client::new();
    let (runtime, _) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, _) = root(&runtime).await;
    let found = invoke(
        &runtime,
        "/drive",
        &service,
        "search",
        json!({"query":"alpha.md"}),
    )
    .await;
    let selected = path(&found["entries"][0]);
    let (file, _) = native(&runtime, selected).await;
    let count = client.0.lock().unwrap().requests.len();
    client.0.lock().unwrap().revoked = true;
    assert!(
        runtime
            .invoke(selected.into(), file, "read".into(), json!({}), execution())
            .await
            .is_err()
    );
    assert!(runtime.inspect(selected.into(), true).await.is_err());
    let state = client.0.lock().unwrap();
    assert!(
        !state.requests[count..]
            .iter()
            .any(|p| p.contains("read-text") || p.contains("read-chunk"))
    );
    assert!(state.mutations.is_empty());
}

fn resolved<'a>(
    descriptor: &'a wip_protocol::InterfaceDescriptor,
    mut ty: &'a wip_protocol::TypeExpr,
) -> &'a wip_protocol::TypeExpr {
    for _ in 0..16 {
        let wip_protocol::TypeExpr::Named { name } = ty else {
            return ty;
        };
        ty = &descriptor
            .types
            .iter()
            .find(|t| t.name == *name)
            .expect("declared result type")
            .definition;
    }
    panic!("result type resolution exceeded bound")
}
fn fields<'a>(
    descriptor: &'a wip_protocol::InterfaceDescriptor,
    ty: &'a wip_protocol::TypeExpr,
) -> &'a [wip_protocol::FieldDeclaration] {
    let wip_protocol::TypeExpr::Record { fields } = resolved(descriptor, ty) else {
        panic!("expected typed result record, got {ty:?}");
    };
    fields
}
fn field<'a>(
    fields: &'a [wip_protocol::FieldDeclaration],
    name: &str,
    required: bool,
) -> &'a wip_protocol::TypeExpr {
    let field = fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("missing public field {name}"));
    assert_eq!(field.required, required, "{name} optionality");
    &field.r#type
}
fn entry_result(descriptor: &wip_protocol::InterfaceDescriptor, ty: &wip_protocol::TypeExpr) {
    let fields = fields(descriptor, ty);
    assert_eq!(fields.len(), 2);
    assert_eq!(field(fields, "path", true), &wip_protocol::TypeExpr::Entry);
    assert_eq!(
        field(fields, "metadata", true),
        &wip_protocol::TypeExpr::Json
    );
}

#[tokio::test]
async fn every_native_result_declares_navigation_outside_opaque_metadata() {
    use wip_protocol::TypeExpr;
    let client = Client::new();
    let (runtime, feature) = runtime(client, Arc::new(WorkdirSessionRouter::new()));
    let (service, root) = root(&runtime).await;
    let created = invoke(
        &runtime,
        "/drive",
        &service,
        "create_text",
        json!({"name":"contract.md","content":"typed"}),
    )
    .await;
    let provider = wip::DriveProvider::new(feature);
    for path in ["/drive", path(&root), path(&created["entry"])] {
        let publication = provider.publication(path).await.unwrap().unwrap();
        let descriptor = &publication.projection.descriptor;
        for operation in &descriptor.operations {
            let ty = &operation.returns.r#type;
            if matches!(operation.name.as_str(), "root" | "metadata") {
                entry_result(descriptor, ty);
                continue;
            }
            let fields = fields(descriptor, ty);
            match operation.name.as_str() {
                "list" | "search" => {
                    let TypeExpr::List { items } =
                        resolved(descriptor, field(fields, "entries", true))
                    else {
                        panic!("page entries must be a typed list");
                    };
                    entry_result(descriptor, items);
                    assert_eq!(field(fields, "next_after", false), &TypeExpr::String);
                }
                "create_folder" | "create_text" | "save_workdir" | "relocate" => {
                    assert_eq!(field(fields, "request_id", true), &TypeExpr::String);
                    entry_result(descriptor, field(fields, "entry", true));
                }
                "delete" => {
                    assert_eq!(fields.len(), 1, "delete cannot return a live Entry");
                    assert_eq!(field(fields, "request_id", true), &TypeExpr::String);
                }
                "request_status" => {
                    assert_eq!(field(fields, "request_id", true), &TypeExpr::String);
                    let TypeExpr::Enum { cases } =
                        resolved(descriptor, field(fields, "state", true))
                    else {
                        panic!("receipt state must be a closed enum");
                    };
                    assert_eq!(
                        cases
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from(["uncommitted", "committed"])
                    );
                    let receipt = self::fields(descriptor, field(fields, "response", false));
                    assert_eq!(field(receipt, "request_id", true), &TypeExpr::String);
                    entry_result(descriptor, field(receipt, "entry", false));
                }
                "read" => {
                    entry_result(descriptor, field(fields, "entry", true));
                    assert_eq!(field(fields, "text", true), &TypeExpr::String);
                    assert_eq!(field(fields, "truncated", true), &TypeExpr::Boolean);
                }
                "view_image" => {
                    entry_result(descriptor, field(fields, "entry", true));
                    assert_eq!(field(fields, "attached", true), &TypeExpr::Boolean);
                    assert_eq!(field(fields, "mime_type", true), &TypeExpr::String);
                    assert_eq!(field(fields, "bytes", true), &TypeExpr::Integer);
                }
                operation => panic!("unaccounted native result contract: {operation}"),
            }
        }
    }
}

#[tokio::test]
async fn read_only_publication_keeps_navigation_but_exposes_no_mutations() {
    let client = Client::new();
    client.0.lock().unwrap().write = false;
    let (runtime, _) = runtime(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let (service, root) = root(&runtime).await;
    let (_, signature) = native(&runtime, "/drive").await;
    assert!(signature.contains("operation search("));
    for operation in ["create_folder", "create_text", "save_workdir"] {
        assert!(!signature.contains(&format!("operation {operation}(")));
        assert!(
            runtime
                .invoke(
                    "/drive".into(),
                    service.clone(),
                    operation.into(),
                    json!({}),
                    execution()
                )
                .await
                .is_err()
        );
    }
    let found = invoke(
        &runtime,
        "/drive",
        &service,
        "search",
        json!({"query":"alpha.md"}),
    )
    .await;
    let selected = path(&found["entries"][0]);
    let (_, signature) = native(&runtime, selected).await;
    assert!(signature.contains("operation read("));
    for operation in ["relocate", "delete", "write", "edit"] {
        assert!(!signature.contains(&format!("operation {operation}(")));
    }
    let object = inspect(&runtime, selected).await;
    for interface in object["interfaces"].as_array().unwrap() {
        assert_ne!(
            interface["reference"]["name"],
            crate::file_operation::Contract::Write.reference().name
        );
        assert_ne!(
            interface["reference"]["name"],
            crate::file_operation::Contract::Edit.reference().name
        );
    }
    let (_, signature) = native(&runtime, path(&root)).await;
    assert!(signature.contains("operation list("));
    assert!(client.0.lock().unwrap().mutations.is_empty());
    assert!(client.0.lock().unwrap().uploads.is_empty());
}

#[derive(Clone)]
struct NoLlm;
#[async_trait]
impl agen::llm_client::LlmClient for NoLlm {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError> {
        panic!("model Tool fixture does not generate requests")
    }
}
async fn model_call(tools: &BTreeMap<String, Arc<dyn Tool>>, name: &str, input: Value) -> Value {
    output(
        tools[name]
            .execute(&input.to_string(), execution())
            .await
            .unwrap(),
    )
}
async fn model_native(tools: &BTreeMap<String, Arc<dyn Tool>>, path: &str) -> Value {
    let object = model_call(tools, "Inspect", json!({"path":path})).await;
    let interface = object["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["reference"]["name"] == NATIVE)
        .unwrap();
    let definition = model_call(tools, "Inspect", json!({"path":interface["path"]})).await;
    assert!(
        definition["interface_signature"]
            .as_str()
            .unwrap()
            .contains("path: entry")
    );
    interface["reference"].clone()
}
async fn model_invoke(
    tools: &BTreeMap<String, Arc<dyn Tool>>,
    path: &str,
    reference: &Value,
    operation: &str,
    arguments: Value,
) -> Value {
    model_call(
        tools,
        "Invoke",
        json!({"path":path,"interface":reference,"operation":operation,"arguments":arguments}),
    )
    .await
}

#[tokio::test]
async fn model_tools_follow_inspect_declared_entries_for_discovery_creation_save_and_move() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("artifact.md"), "saved from model Tool").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            workdir::WorkdirAttachmentAlias::new("source").unwrap(),
            Arc::new(workdir::LocalWorkdirSession::new(
                manifest::Scope::from_config(
                    &serde_json::from_value(
                        json!({"allow":[{"target":dir.path(),"permission":"read"}]}),
                    )
                    .unwrap(),
                )
                .unwrap(),
                dir.path().into(),
            )),
        )
        .unwrap();
    let client = Client::new();
    let feature = DriveFeature::new(client.clone(), router);
    let mut mounts = WipMountRegistry::new();
    wip::mount_drive_wip(&mut mounts, &feature).unwrap();
    let mut engine = agen::Engine::<_, agen::state::Mutable, ()>::new_annotated(NoLlm);
    engine.register_tools(
        feature
            .tools()
            .into_iter()
            .map(|(_, definition)| definition),
    );
    crate::wip::install_wip_mode_with_mounts(&mut engine, None, "drive-model-tools".into(), mounts)
        .unwrap();
    let handle = engine.tool_server_handle();
    handle.flush_pending();
    assert!(handle.get_tool("DriveRead").is_none());
    let tools: BTreeMap<_, _> = ["Tree", "Inspect", "Invoke"]
        .into_iter()
        .map(|name| (name.into(), handle.get_tool(name).unwrap().1))
        .collect();
    let service = model_native(&tools, "/drive").await;
    let root = model_invoke(&tools, "/drive", &service, "root", json!({})).await;
    let root_ref = model_native(&tools, path(&root)).await;
    let folders = model_invoke(&tools, path(&root), &root_ref, "list", json!({"limit":1})).await;
    let folder = &folders["entries"][0];
    let folder_ref = model_native(&tools, path(folder)).await;
    let first = model_invoke(
        &tools,
        path(folder),
        &folder_ref,
        "list",
        json!({"limit":1}),
    )
    .await;
    let next = model_invoke(
        &tools,
        path(folder),
        &folder_ref,
        "list",
        json!({"limit":1,"after":first["next_after"]}),
    )
    .await;
    let found = model_invoke(
        &tools,
        "/drive",
        &service,
        "search",
        json!({"query":".md","limit":1}),
    )
    .await;
    let second = model_invoke(
        &tools,
        "/drive",
        &service,
        "search",
        json!({"query":".md","limit":1,"after":found["next_after"]}),
    )
    .await;
    assert_eq!(second["entries"][0], next["entries"][0]);
    let selected = path(&second["entries"][0]);
    let file_ref = model_native(&tools, selected).await;
    assert_eq!(
        model_invoke(&tools, selected, &file_ref, "read", json!({})).await["text"],
        "beta body"
    );
    let destination = model_invoke(
        &tools,
        path(&root),
        &root_ref,
        "create_folder",
        json!({"name":"published"}),
    )
    .await;
    let dest = path(&destination["entry"]);
    let dest_ref = model_native(&tools, dest).await;
    let created = model_invoke(
        &tools,
        path(&root),
        &root_ref,
        "create_text",
        json!({"name":"new.md","content":"new body"}),
    )
    .await;
    let new_path = path(&created["entry"]);
    let new_ref = model_native(&tools, new_path).await;
    assert_eq!(
        model_invoke(&tools, new_path, &new_ref, "read", json!({})).await["text"],
        "new body"
    );
    let moved = model_invoke(
        &tools,
        new_path,
        &new_ref,
        "relocate",
        json!({"parent":dest,"name":"moved.md"}),
    )
    .await;
    assert_eq!(path(&moved["entry"]), new_path);
    let saved = model_invoke(&tools, dest, &dest_ref, "save_workdir", json!({"target_workdir":"source","path":"artifact.md","name":"artifact.md","content_type":"text/markdown"})).await;
    let saved_path = path(&saved["entry"]);
    let saved_ref = model_native(&tools, saved_path).await;
    assert_eq!(
        model_invoke(&tools, saved_path, &saved_ref, "read", json!({})).await["text"],
        "saved from model Tool"
    );
    let tree = model_call(&tools, "Tree", json!({"path":"/drive","depth":8})).await;
    assert_eq!(tree["coverage"]["complete"], true);
    for path in [selected, dest, new_path, saved_path] {
        assert!(!tree["tree"].to_string().contains(path));
    }
    let state = client.0.lock().unwrap();
    assert_eq!(state.mutations.len(), 3);
    assert_eq!(state.uploads.len(), 1);
}
