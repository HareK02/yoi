//! Worker-side E2E across real Feature invocation, WIP Client/Host and typed
//! Workspace transport. Canonical persistence/projection tests live in Backend;
//! this deterministic router models its published auth/CAS/response contract.
use super::*;
use crate::feature::FeatureRegistryBuilder;
use crate::hook::HookRegistryBuilder;
use crate::wip::{WipMountRegistry, WipRuntime};
use crate::worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
    WorkspaceResponse,
};
use agen::tool::ToolExecutionContext;
use protocol::{FeatureInvocation, FeatureInvocationIdentity};
use serde::Serialize;
use serde_json::{Value as Json, json};
use server_api::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct State {
    granted: bool,
    read_only: bool,
    active: Option<WorkspaceConfigAttachment>,
    next_connection: u64,
    revision: u64,
    entries: BTreeMap<String, (ConfigContentType, String)>,
    commits: Vec<WorkspaceConfigCommitRequest>,
    requests: Vec<WorkspaceRequest>,
    fail_commit: bool,
    fail_current: bool,
    lose_commit_reply: bool,
    race_commit: bool,
}
#[derive(Debug)]
struct Router {
    workspace: &'static str,
    state: Arc<Mutex<State>>,
}
impl Router {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            workspace: "workspace-1",
            state: Arc::new(Mutex::new(State {
                granted: true,
                read_only: false,
                active: None,
                next_connection: 0,
                revision: 1,
                entries: BTreeMap::from([
                    (
                        "main.dcdl".into(),
                        (ConfigContentType::Decodal, "model = old".into()),
                    ),
                    (
                        "profiles/team/deep/model.dcdl".into(),
                        (ConfigContentType::Decodal, "model = deep".into()),
                    ),
                ]),
                commits: Vec::new(),
                requests: Vec::new(),
                fail_commit: false,
                fail_current: false,
                lose_commit_reply: false,
                race_commit: false,
            })),
        })
    }
}
fn response<T: Serialize>(value: &T) -> WorkspaceResponse {
    WorkspaceResponse {
        status: 200,
        body: serde_json::to_string(value).unwrap(),
    }
}
fn denied(status: u16, classification: WorkspaceConfigFailureClassification) -> WorkspaceResponse {
    // Deliberately unsafe provider text must never be propagated by the adapter.
    WorkspaceResponse {
        status,
        body: serde_json::to_string(&WorkspaceConfigApiError {
            status,
            code: "test".into(),
            message: "secret-token /private/host/config".into(),
            classification,
        })
        .unwrap(),
    }
}
fn digest(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
impl State {
    fn tree_digest(&self) -> String {
        digest(&serde_json::to_string(&self.entries).unwrap())
    }
    fn validator(&self, path: &str) -> String {
        format!(
            "wc:{}:{}:{}:{}",
            self.active.as_ref().unwrap().connection_id,
            self.revision,
            self.tree_digest(),
            path
        )
    }
    fn node(&self, path: &str, kind: WorkspaceConfigNodeKind) -> WorkspaceConfigNode {
        let mut operations = match kind {
            WorkspaceConfigNodeKind::File => vec!["read".into()],
            _ => vec![],
        };
        if !self.read_only {
            match kind {
                WorkspaceConfigNodeKind::File => {
                    operations.extend(["write", "edit", "delete"].map(str::to_string))
                }
                WorkspaceConfigNodeKind::Missing => operations.push("create".into()),
                WorkspaceConfigNodeKind::Directory => {
                    operations.push("create".into());
                    if path.is_empty() {
                        operations.push("apply_changes".into());
                    }
                }
            }
        }
        WorkspaceConfigNode {
            path: path.into(),
            kind,
            validator: self.validator(path),
            digest: self.entries.get(path).map(|(_, s)| digest(s)),
            content_type: self.entries.get(path).map(|(t, _)| *t),
            operations,
        }
    }
}
impl WorkspaceClient for Router {
    fn workspace_id(&self) -> Option<&str> {
        Some(self.workspace)
    }
    fn kind(&self) -> &str {
        "typed-config-router"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let mut state = self.state.lock().unwrap();
        state.requests.push(request.clone());
        let prefix = "/api/w/workspace-1/workers/self/workspace-config";
        if self.workspace != "workspace-1" || !request.path.starts_with(prefix) || !state.granted {
            return Ok(denied(
                403,
                WorkspaceConfigFailureClassification::NotCommitted,
            ));
        }
        let suffix = request.path.strip_prefix(prefix).unwrap();
        if suffix.is_empty() {
            if request.method == WorkspaceRequestMethod::Get {
                if state.fail_current {
                    return Ok(WorkspaceResponse {
                        status: 503,
                        body: "unsafe provider diagnostic /private/host/config".into(),
                    });
                }
                return Ok(response(&state.active));
            }
            let attach: WorkspaceConfigAttachRequest =
                serde_json::from_str(request.body.as_ref().unwrap()).unwrap();
            assert_eq!(attach.alias.as_deref(), Some(ATTACHMENT_ALIAS));
            if attach.access == Some(WorkspaceConfigAccess::ReadWrite) && state.read_only {
                return Ok(denied(
                    403,
                    WorkspaceConfigFailureClassification::NotCommitted,
                ));
            }
            let already_attached = state.active.is_some();
            if !already_attached {
                state.next_connection += 1;
            }
            let access = attach.access.unwrap_or(if state.read_only {
                WorkspaceConfigAccess::ReadOnly
            } else {
                WorkspaceConfigAccess::ReadWrite
            });
            state.read_only |= access == WorkspaceConfigAccess::ReadOnly;
            let attachment = WorkspaceConfigAttachment {
                workspace_id: "workspace-1".into(),
                connection_id: format!("connection-{}", state.next_connection),
                alias: ATTACHMENT_ALIAS.into(),
                working_directory_id: "logical-grant-1".into(),
                access,
                name: "Workspace configuration".into(),
                purpose: "Canonical config".into(),
                content_path: CONTENT_ROOT.into(),
                already_attached,
            };
            state.active = Some(attachment.clone());
            return Ok(response(&attachment));
        }
        let input: Json = serde_json::from_str(request.body.as_ref().unwrap()).unwrap();
        if state.active.as_ref().map(|a| a.connection_id.as_str())
            != input["connection_id"].as_str()
        {
            return Ok(denied(
                409,
                WorkspaceConfigFailureClassification::NotCommitted,
            ));
        }
        match suffix {
            "/observe" => {
                let request: WorkspaceConfigObserveRequest = serde_json::from_value(input).unwrap();
                assert!(request.depth <= 8 && request.paths.len() <= 256);
                let mut kinds =
                    BTreeMap::from([(String::new(), WorkspaceConfigNodeKind::Directory)]);
                for path in state.entries.keys() {
                    kinds.insert(path.clone(), WorkspaceConfigNodeKind::File);
                    let mut path = path.as_str();
                    while let Some((parent, _)) = path.rsplit_once('/') {
                        kinds
                            .entry(parent.into())
                            .or_insert(WorkspaceConfigNodeKind::Directory);
                        path = parent;
                    }
                }
                let mut selected = BTreeSet::new();
                for path in &request.paths {
                    selected.insert(path.clone());
                    let prefix = if path.is_empty() {
                        String::new()
                    } else {
                        format!("{path}/")
                    };
                    for child in kinds.keys() {
                        if let Some(suffix) = child.strip_prefix(&prefix)
                            && child != path
                            && suffix.split('/').count() <= request.depth as usize
                        {
                            selected.insert(child.clone());
                        }
                    }
                }
                if selected.len() > 256 {
                    return Ok(denied(
                        413,
                        WorkspaceConfigFailureClassification::NotCommitted,
                    ));
                }
                let nodes = selected
                    .into_iter()
                    .map(|p| {
                        state.node(
                            &p,
                            *kinds.get(&p).unwrap_or(&WorkspaceConfigNodeKind::Missing),
                        )
                    })
                    .collect();
                Ok(response(&WorkspaceConfigObserveResponse {
                    connection_id: state.active.as_ref().unwrap().connection_id.clone(),
                    validator: state.validator(""),
                    revision: state.revision,
                    digest: state.tree_digest(),
                    entrypoints: vec!["main.dcdl".into()],
                    nodes,
                }))
            }
            "/read" => {
                let request: WorkspaceConfigReadRequest = serde_json::from_value(input).unwrap();
                if request.validator != state.validator(&request.path) {
                    return Ok(denied(
                        409,
                        WorkspaceConfigFailureClassification::NotCommitted,
                    ));
                }
                let Some((content_type, content)) = state.entries.get(&request.path) else {
                    return Ok(denied(
                        404,
                        WorkspaceConfigFailureClassification::NotCommitted,
                    ));
                };
                Ok(response(&WorkspaceConfigReadResponse {
                    path: request.path.clone(),
                    content: content.clone(),
                    content_type: *content_type,
                    digest: digest(content),
                    validator: state.validator(&request.path),
                }))
            }
            "/commit" => {
                let request: WorkspaceConfigCommitRequest = serde_json::from_value(input).unwrap();
                state.commits.push(request.clone());
                if state.race_commit {
                    state.revision += 1;
                    state.race_commit = false;
                }
                if state.read_only {
                    return Ok(denied(
                        403,
                        WorkspaceConfigFailureClassification::NotCommitted,
                    ));
                }
                assert_eq!(request.request.entrypoints, ["main.dcdl"]);
                if request.validator != state.validator("")
                    || request.request.base_revision != state.revision
                    || request.request.base_digest != state.tree_digest()
                {
                    return Ok(denied(
                        409,
                        WorkspaceConfigFailureClassification::NotCommitted,
                    ));
                }
                if state.fail_commit {
                    return Ok(denied(
                        500,
                        WorkspaceConfigFailureClassification::NotCommitted,
                    ));
                }
                let mut entries = state.entries.clone();
                for change in request.request.changes {
                    match change {
                        ConfigTreeChange::Create {
                            path,
                            content_type,
                            content,
                        } => {
                            if entries.contains_key(&path) {
                                return Ok(denied(
                                    409,
                                    WorkspaceConfigFailureClassification::NotCommitted,
                                ));
                            }
                            entries.insert(path, (content_type, content));
                        }
                        ConfigTreeChange::Update {
                            path,
                            expected_digest,
                            content,
                        } => {
                            let Some((kind, old)) = entries.get(&path) else {
                                return Ok(denied(
                                    409,
                                    WorkspaceConfigFailureClassification::NotCommitted,
                                ));
                            };
                            if digest(old) != expected_digest {
                                return Ok(denied(
                                    409,
                                    WorkspaceConfigFailureClassification::NotCommitted,
                                ));
                            }
                            entries.insert(path, (*kind, content));
                        }
                        ConfigTreeChange::Delete {
                            path,
                            expected_digest,
                        } => {
                            if entries
                                .get(&path)
                                .is_none_or(|(_, s)| digest(s) != expected_digest)
                            {
                                return Ok(denied(
                                    409,
                                    WorkspaceConfigFailureClassification::NotCommitted,
                                ));
                            }
                            entries.remove(&path);
                        }
                        ConfigTreeChange::Rename {
                            from,
                            to,
                            expected_digest,
                        } => {
                            if entries
                                .get(&from)
                                .is_none_or(|(_, s)| digest(s) != expected_digest)
                                || entries.contains_key(&to)
                            {
                                return Ok(denied(
                                    409,
                                    WorkspaceConfigFailureClassification::NotCommitted,
                                ));
                            }
                            let entry = entries.remove(&from).unwrap();
                            entries.insert(to, entry);
                        }
                    }
                }
                if entries.values().any(|(_, s)| s == "INVALID") {
                    return Ok(denied(
                        400,
                        WorkspaceConfigFailureClassification::NotCommitted,
                    ));
                }
                state.entries = entries;
                state.revision += 1;
                if state.lose_commit_reply {
                    state.lose_commit_reply = false;
                    return Err(WorkspaceClientError::Request(
                        "secret-token /private/host/config".into(),
                    ));
                }
                Ok(response(&WorkspaceConfigCommitResponse {
                    validator: state.validator(""),
                    revision: state.revision,
                    digest: state.tree_digest(),
                }))
            }
            _ => panic!("unregistered endpoint: {suffix}"),
        }
    }
}
fn invocation(key: &str) -> FeatureInvocation {
    FeatureInvocation {
        invocation_id: key.into(),
        identity: FeatureInvocationIdentity(INVOCATION_ID.into()),
        name: FEATURE_ID.into(),
        arguments: vec![],
    }
}
fn runtime(client: Arc<dyn WorkspaceClient>) -> (WorkspaceConfigFeature, WipRuntime) {
    let feature = WorkspaceConfigFeature::for_workspace(client, true);
    let mut mounts = WipMountRegistry::new();
    wip::mount_workspace_config_wip(&mut mounts, &feature).unwrap();
    let runtime = WipRuntime::from_mounts(mounts, "worker-1@workspace-1".into()).unwrap();
    (feature, runtime)
}
fn interface(path: &str, attached: bool) -> wip_protocol::InterfaceReference {
    crate::wip::contextual_reference(
        if attached {
            "yoi.workspace-config/node/v1"
        } else {
            "yoi.workspace-config/attachment/v1"
        },
        path,
    )
}
async fn observe_interface(runtime: &WipRuntime, path: &str, _attached: bool) {
    runtime.tree(path.into(), 0, true).await.unwrap();
    runtime.inspect(path.into(), true).await.unwrap();
}
async fn call(
    runtime: &WipRuntime,
    path: &str,
    op: &str,
    args: Json,
) -> Result<agen::tool::ToolOutput, agen::tool::ToolError> {
    runtime
        .call(
            path.into(),
            interface(path, true),
            op.into(),
            args,
            ToolExecutionContext::direct(),
        )
        .await
}

#[tokio::test]
async fn workspace_config_production_slash_deep_discover_read_edit_and_atomic_import_changes() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    let mut tools = Vec::new();
    let report = FeatureRegistryBuilder::new()
        .with_module(feature)
        .install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
    assert!(tools.is_empty() && !report.has_errors());
    assert_eq!(
        report
            .chat_invocations
            .feature_completions("workspace-c")
            .len(),
        1
    );
    assert!(
        report
            .chat_invocations
            .invoke(&invocation("slash-1"))
            .await
            .unwrap()
            .message
            .starts_with("Attached")
    );
    assert!(
        report
            .chat_invocations
            .invoke(&invocation("slash-2"))
            .await
            .unwrap()
            .message
            .starts_with("Already attached")
    );
    let discovered = runtime
        .tree(CONTENT_ROOT.into(), 5, true)
        .await
        .unwrap()
        .content
        .unwrap();
    assert!(discovered.contains("profiles/team/deep/model.dcdl"));
    assert!(!discovered.contains("model = deep"));
    let path = "/workspace-config/profiles/team/deep/model.dcdl";
    observe_interface(&runtime, path, true).await;
    assert!(
        call(&runtime, path, "read", json!({}))
            .await
            .unwrap()
            .content
            .unwrap()
            .contains("model = deep")
    );
    call(
        &runtime,
        path,
        "edit",
        json!({"old_string":"deep","new_string":"changed"}),
    )
    .await
    .unwrap();
    assert_eq!(
        client.state.lock().unwrap().entries["profiles/team/deep/model.dcdl"].1,
        "model = changed"
    );
    // Client receives the new path validator automatically, no model tracking.
    call(&runtime, path, "write", json!({"content":"model = second"}))
        .await
        .unwrap();
    observe_interface(&runtime, CONTENT_ROOT, true).await;
    call(
        &runtime,
        CONTENT_ROOT,
        "apply_changes",
        json!({"changes":[
            {"kind":"create","path":"profiles/new.dcdl","content":"model = imported"},
            {"kind":"update","path":"main.dcdl","content":"import profiles/new.dcdl"}
        ]}),
    )
    .await
    .unwrap();
    let state = client.state.lock().unwrap();
    assert_eq!(state.entries["main.dcdl"].1, "import profiles/new.dcdl");
    assert_eq!(state.entries["profiles/new.dcdl"].1, "model = imported");
    assert_eq!(state.commits.last().unwrap().request.changes.len(), 2);
    assert!(
        state
            .requests
            .iter()
            .all(|r| !r.path.contains("session") && !r.path.contains("bash"))
    );
}

#[tokio::test]
async fn workspace_config_production_native_attach_create_delete_and_detach_end_use() {
    let client = Router::new();
    let (_, runtime) = runtime(client.clone());
    observe_interface(&runtime, CONTENT_ROOT, false).await;
    runtime
        .call(
            CONTENT_ROOT.into(),
            interface(CONTENT_ROOT, false),
            "attach".into(),
            json!({}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let directory = "/workspace-config/profiles";
    observe_interface(&runtime, directory, true).await;
    call(
        &runtime,
        directory,
        "create",
        json!({"path":"notes.txt", "content_type":"text", "content":"configuration notes"}),
    )
    .await
    .unwrap();
    assert_eq!(
        client.state.lock().unwrap().entries["profiles/notes.txt"].0,
        ConfigContentType::Text
    );
    let path = "/workspace-config/new/nested/file.dcdl";
    observe_interface(&runtime, path, true).await;
    call(&runtime, path, "create", json!({"content":"new = true"}))
        .await
        .unwrap();
    observe_interface(&runtime, path, true).await;
    call(&runtime, path, "delete", json!({})).await.unwrap();
    assert!(
        !client
            .state
            .lock()
            .unwrap()
            .entries
            .contains_key("new/nested/file.dcdl")
    );
    client.state.lock().unwrap().active = None;
    assert!(
        call(&runtime, "/workspace-config/main.dcdl", "read", json!({}))
            .await
            .is_err()
    );
    assert!(
        runtime
            .tree("/workspace-config/main.dcdl".into(), 0, true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn workspace_config_production_revocation_ro_cross_workspace_and_cache_isolation() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    feature.attach("grant", None).await.unwrap();
    let path = "/workspace-config/main.dcdl";
    observe_interface(&runtime, path, true).await;
    client.state.lock().unwrap().granted = false;
    assert!(call(&runtime, path, "read", json!({})).await.is_err());
    assert!(feature.attach("revoked", None).await.is_err());
    client.state.lock().unwrap().granted = true;
    client.state.lock().unwrap().read_only = true;
    client.state.lock().unwrap().active.as_mut().unwrap().access = WorkspaceConfigAccess::ReadOnly;
    assert!(
        feature
            .attach("explicit-write", Some(ConfigAccess::ReadWrite))
            .await
            .is_err()
    );
    observe_interface(&runtime, path, true).await;
    let descriptor = runtime
        .inspect(path.into(), true)
        .await
        .unwrap()
        .content
        .unwrap();
    let descriptor: Json = serde_json::from_str(&descriptor).unwrap();
    let signature = descriptor["interfaces"][0]["signature"].as_str().unwrap();
    assert!(signature.contains("operation read("));
    assert!(!signature.contains("operation write("));
    assert!(
        call(&runtime, path, "write", json!({"content":"DENIED"}))
            .await
            .is_err()
    );
    let cross = Arc::new(Router {
        workspace: "other-workspace",
        state: client.state.clone(),
    });
    let (feature, other) = runtime_for_cross(cross);
    assert!(feature.attach("cross", None).await.is_err());
    assert!(other.tree(path.into(), 0, false).await.is_err());
    assert!(client.state.lock().unwrap().commits.is_empty());
}
fn runtime_for_cross(client: Arc<dyn WorkspaceClient>) -> (WorkspaceConfigFeature, WipRuntime) {
    runtime(client)
}

#[tokio::test]
async fn workspace_config_production_stale_same_bytes_and_commit_race_never_overwrite() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    feature.attach("initial", None).await.unwrap();
    let path = "/workspace-config/main.dcdl";
    observe_interface(&runtime, path, true).await;
    client.state.lock().unwrap().revision += 1; // Same bytes, different identity revision.
    assert!(
        call(&runtime, path, "write", json!({"content":"overwritten"}))
            .await
            .is_err()
    );
    assert!(client.state.lock().unwrap().commits.is_empty());
    observe_interface(&runtime, path, true).await;
    client.state.lock().unwrap().race_commit = true;
    assert!(
        call(&runtime, path, "write", json!({"content":"raced"}))
            .await
            .is_err()
    );
    assert_eq!(
        client.state.lock().unwrap().entries["main.dcdl"].1,
        "model = old"
    );
    assert_eq!(client.state.lock().unwrap().commits.len(), 1);
}

#[tokio::test]
async fn workspace_config_production_invalid_size_save_failure_unknown_are_safe_and_not_retried() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    feature.attach("initial", None).await.unwrap();
    let path = "/workspace-config/main.dcdl";
    for content in ["INVALID".into(), "x".repeat(256 * 1024 + 1)] {
        observe_interface(&runtime, path, true).await;
        let error = call(&runtime, path, "write", json!({"content":content}))
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret-token") && !error.contains("/private/host"));
        assert_eq!(
            client.state.lock().unwrap().entries["main.dcdl"].1,
            "model = old"
        );
    }
    observe_interface(&runtime, path, true).await;
    let before = client.state.lock().unwrap().commits.len();
    assert!(
        call(
            &runtime,
            path,
            "write",
            json!({"content":"valid","validator":"model-supplied"})
        )
        .await
        .is_err()
    );
    assert_eq!(before, client.state.lock().unwrap().commits.len());
    client.state.lock().unwrap().fail_commit = true;
    assert!(
        call(&runtime, path, "write", json!({"content":"valid"}))
            .await
            .is_err()
    );
    assert_eq!(
        client.state.lock().unwrap().entries["main.dcdl"].1,
        "model = old"
    );
    client.state.lock().unwrap().fail_commit = false;
    observe_interface(&runtime, path, true).await;
    client.state.lock().unwrap().lose_commit_reply = true;
    let count = client.state.lock().unwrap().commits.len();
    let error = call(
        &runtime,
        path,
        "write",
        json!({"content":"saved-but-reply-lost"}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("unknown"));
    assert!(!error.contains("secret-token") && !error.contains("/private/host"));
    assert_eq!(client.state.lock().unwrap().commits.len(), count + 1);
    assert_eq!(
        client.state.lock().unwrap().entries["main.dcdl"].1,
        "saved-but-reply-lost"
    );
}

#[tokio::test]
async fn workspace_config_production_bounds_and_traversal_reject_before_content_calls() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    feature.attach("initial", None).await.unwrap();
    assert!(runtime.tree(CONTENT_ROOT.into(), 9, true).await.is_err());
    for path in [
        "/workspace-config/../secret",
        "/workspace-config//main.dcdl",
        "/workspace-config/./main.dcdl",
    ] {
        assert!(runtime.tree(path.into(), 0, true).await.is_err());
    }
    let path = format!("/workspace-config/{}", "a".repeat(513));
    assert!(runtime.tree(path, 0, true).await.is_err());
    assert!(client.state.lock().unwrap().commits.is_empty());
}

#[tokio::test]
async fn workspace_config_production_reattach_new_lifetime_rejects_old_same_bytes_cache() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    let first = feature.attach("first", None).await.unwrap();
    let path = "/workspace-config/main.dcdl";
    observe_interface(&runtime, path, true).await;
    client.state.lock().unwrap().active = None;
    let second = feature.attach("second", None).await.unwrap();
    assert_ne!(first.connection_id, second.connection_id);
    assert!(
        call(&runtime, path, "write", json!({"content":"bad"}))
            .await
            .is_err()
    );
    assert!(client.state.lock().unwrap().commits.is_empty());
    observe_interface(&runtime, path, true).await;
    call(&runtime, path, "write", json!({"content":"new-lifetime"}))
        .await
        .unwrap();
    assert_eq!(
        client.state.lock().unwrap().entries["main.dcdl"].1,
        "new-lifetime"
    );
}

#[tokio::test]
async fn workspace_config_production_cumulative_node_limit_and_edit_growth_are_bounded() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    feature.attach("initial", None).await.unwrap();
    let path = "/workspace-config/main.dcdl";
    observe_interface(&runtime, path, true).await;
    let before = client.state.lock().unwrap().commits.len();
    assert!(
        call(
            &runtime,
            path,
            "edit",
            json!({
                "old_string":"o", "new_string":"x".repeat(256*1024), "replace_all":true
            })
        )
        .await
        .is_err()
    );
    assert_eq!(client.state.lock().unwrap().commits.len(), before);
    {
        let mut state = client.state.lock().unwrap();
        for i in 0..90 {
            state.entries.insert(
                format!("groups/{i}/nested/f.dcdl"),
                (ConfigContentType::Decodal, "x = true".into()),
            );
        }
    }
    // No individual depth-1 response exceeds the cap; the whole Host traversal
    // must nevertheless stop before exceeding 256 Objects.
    let error = runtime
        .tree(CONTENT_ROOT.into(), 4, true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ResourceLimitExceeded"));
}

#[tokio::test]
async fn workspace_config_production_backend_failure_is_not_hidden_as_unauthorized_absence() {
    let client = Router::new();
    client.state.lock().unwrap().fail_current = true;
    let (_, runtime) = runtime(client);
    let error = runtime
        .tree(CONTENT_ROOT.into(), 0, true)
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("Internal"), "{message}");
    assert!(!message.contains("/private/host/config"));
}

#[tokio::test]
async fn workspace_config_edit_shared_argument_rejection_never_commits() {
    let client = Router::new();
    let (feature, runtime) = runtime(client.clone());
    feature.attach("initial", None).await.unwrap();
    let path = "/workspace-config/main.dcdl";
    observe_interface(&runtime, path, true).await;
    for (args, expected) in [
        (
            json!({"old_string":"old","new_string":"old"}),
            "old_string and new_string are identical",
        ),
        (
            json!({"old_string":"old","new_string":"new","replace_all":"true"}),
            "expected boolean",
        ),
    ] {
        let error = call(&runtime, path, "edit", args).await.unwrap_err();
        assert!(error.to_string().contains(expected), "{error:?}");
        let state = client.state.lock().unwrap();
        assert!(state.commits.is_empty());
        assert_eq!(state.revision, 1);
        assert_eq!(state.entries["main.dcdl"].1, "model = old");
        // Argument-only rejection must not even fetch a preimage.
        assert!(
            state
                .requests
                .iter()
                .all(|request| !request.path.ends_with("/read"))
        );
    }
    // Rejection has not made the observation stale or poisoned the commit path.
    call(
        &runtime,
        path,
        "edit",
        json!({"old_string":"old","new_string":"新"}),
    )
    .await
    .unwrap();
    let state = client.state.lock().unwrap();
    assert_eq!(state.commits.len(), 1);
    assert_eq!(state.entries["main.dcdl"].1, "model = 新");
}

#[tokio::test]
async fn workspace_config_production_ungranted_root_does_not_poison_worldspace_discovery() {
    let client = Router::new();
    client.state.lock().unwrap().granted = false;
    let (_, runtime) = runtime(client.clone());
    let discovery = runtime.tree("/".into(), 2, true).await.unwrap();
    let discovery: Json = serde_json::from_str(discovery.content.as_deref().unwrap()).unwrap();
    assert_eq!(discovery["tree"]["children"], json!([]));
    assert_eq!(discovery["tree"]["path"], "/");
    assert!(runtime.tree(CONTENT_ROOT.into(), 0, true).await.is_err());
    assert!(
        client
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|r| r.method == WorkspaceRequestMethod::Get)
    );
}
